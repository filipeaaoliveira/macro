use std::collections::BTreeMap;
use std::convert::Infallible;

use factory_records::domain::models::{AgentName, EntryId, Role};
use factory_records::domain::standing_rules::OrchestratorDefinition;

use super::{
    DEFINITION_FILE_PATTERNS, DefinitionError, DefinitionFiles, DefinitionReadError,
    FactoryDefinition, MAX_KNOWN_FLAKES, read_definition,
};
use crate::domain::agents::AgentDefinitionError;
use crate::domain::built_in_steps::built_in_step_use;
use crate::domain::control_files::{ControlFiles, ControlFilesError};
use crate::domain::evaluation::{EvaluationDefinition, EvaluationDefinitionError};
use crate::domain::flow::FlowError;
use crate::outbound::local_definition_files::LocalDefinitionFiles;

/// A Definition's files, by path.
struct Files(BTreeMap<String, String>);

impl DefinitionFiles for Files {
    type Err = Infallible;

    async fn read(&self, path: &str) -> Result<Option<String>, Infallible> {
        Ok(self.0.get(path).cloned())
    }
}

const MANIFEST: &str = r#"{ "version": 1, "description": "Ours.", "flows": ["reply", "plan"] }"#;

const AGENTS: &str = r#"{
    "agents": {
        "primary": { "bot": "00000000-0000-0000-0000-00000000c1a0", "model": "large-model" },
        "backup": { "bot": "00000000-0000-0000-0000-00000000c0d1" }
    },
    "roles": { "planner": ["primary", "backup"], "writer": ["primary"] }
}"#;

const PLAN: &str = include_str!("../../../flows/plan.json");

const REPLY: &str = include_str!("../../../flows/reply.json");

/// A Definition of the `reply` and `plan` Flows, its Agents filling the
/// plan Flow's Roles, with no rules or Scorers.
fn files() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("factory.json".to_string(), MANIFEST.to_string()),
        ("flows/reply.json".to_string(), REPLY.to_string()),
        ("flows/plan.json".to_string(), PLAN.to_string()),
        ("agents.json".to_string(), AGENTS.to_string()),
    ])
}

async fn read(files: BTreeMap<String, String>) -> Result<FactoryDefinition, DefinitionError> {
    match read_definition(&Files(files), built_in_step_use).await {
        Ok(definition) => Ok(definition),
        Err(DefinitionReadError::Invalid(error)) => Err(error),
        Err(DefinitionReadError::Files(never)) => match never {},
    }
}

fn with(path: &str, text: &str) -> BTreeMap<String, String> {
    let mut files = files();
    files.insert(path.to_string(), text.to_string());
    files
}

#[tokio::test]
async fn a_definition_reads_its_flows_agents_rules_and_scorers() {
    let mut files = with(
        "rules.json",
        r#"{
            "standingRules": { "plan": false, "blockers": true, "merge": false, "queueLimit": 5 },
            "controlFiles": [".github/**", "tests/**"]
        }"#,
    );
    files.insert(
        "scorers.json".to_string(),
        r#"{
            "scorers": [{
                "name": "tests-pass", "instructions": "Do the tests pass?",
                "flows": ["plan"], "samplePercent": 10
            }],
            "experiments": [{
                "name": "backup-writer", "flow": "plan",
                "arms": [{ "name": "usual" }, { "name": "backup", "agents": { "writer": "backup" } }]
            }]
        }"#
        .to_string(),
    );

    let definition = read(files).await.expect("a definition");

    assert_eq!(definition.description, "Ours.");
    assert_eq!(
        definition.flows.keys().collect::<Vec<_>>(),
        ["plan", "reply"]
    );
    assert_eq!(definition.flows["plan"].entries[0].id.as_str(), "plan");
    let planner = Role::try_from("planner".to_string()).expect("a role");
    let primary = AgentName::try_from("primary".to_string()).expect("an agent");
    assert_eq!(definition.agents.for_role(&planner)[0], primary);
    assert_eq!(
        definition.rules.standing_rules,
        Some(OrchestratorDefinition {
            blockers: true,
            queue_limit: Some(5),
            token_budget: None,
            ..OrchestratorDefinition::default()
        })
    );
    assert_eq!(
        definition.rules.control_files,
        Some(vec![".github/**".to_string(), "tests/**".to_string()])
    );
    let scorer = &definition.evaluation.scorers()[0];
    assert_eq!(scorer.name, "tests-pass");
    assert_eq!(scorer.flows, ["plan"]);
    assert_eq!(scorer.sample_percent, 10);
    assert_eq!(definition.evaluation.experiments()[0].flow, "plan");
}

#[tokio::test]
async fn rules_and_scorers_left_out_take_the_defaults() {
    let definition = read(files()).await.expect("a definition");

    assert_eq!(definition.rules.standing_rules, None);
    assert_eq!(definition.rules.control_files, None);
    assert_eq!(definition.evaluation, EvaluationDefinition::default());
}

/// The Definition template, as shipped.
fn template() -> LocalDefinitionFiles {
    LocalDefinitionFiles::new(concat!(env!("CARGO_MANIFEST_DIR"), "/definition_template"))
}

/// A team's copy of a Definition whose placeholder bot ids it replaced
/// with real ones, and changed nothing else.
struct Customized(LocalDefinitionFiles);

impl DefinitionFiles for Customized {
    type Err = std::io::Error;

    async fn read(&self, path: &str) -> Result<Option<String>, std::io::Error> {
        let text = self.0.read(path).await?;
        if path != "agents.json" {
            return Ok(text);
        }
        Ok(text.map(|text| {
            text.replace("00000000-0000-0000-0000-000000000001", PRIMARY_BOT)
                .replace("00000000-0000-0000-0000-000000000002", BACKUP_BOT)
        }))
    }
}

const PRIMARY_BOT: &str = "00000000-0000-0000-0000-00000000c1a0";

const BACKUP_BOT: &str = "00000000-0000-0000-0000-00000000c0d1";

#[tokio::test]
async fn the_starter_definition_reads() {
    let definition = read_definition(&Customized(template()), built_in_step_use)
        .await
        .expect("the starter definition reads once its bots are replaced");

    assert_eq!(
        definition.flows.keys().collect::<Vec<_>>(),
        ["code", "plan", "reply"]
    );
    // A code Run ends at its merge: the starter has no post-merge Steps.
    let steps: Vec<&str> = definition.flows["code"]
        .entries
        .iter()
        .map(|entry| entry.step.as_str())
        .collect();
    assert!(steps.contains(&"accept") && steps.contains(&"merge"));
    assert!(!steps.contains(&"deploy") && !steps.contains(&"verify"));
    let merge_by_rule = definition
        .rules
        .standing_rules
        .is_some_and(|rules| rules.merge);
    assert!(!merge_by_rule, "a person merges in the starter definition");
    // The starter's two Scorers judge a quarter of the code Flow's Runs:
    // whether the change delivered the Done-when clauses, and whether the
    // review's Blockers can be acted on.
    let scorers: Vec<(&str, &[String], u8)> = definition
        .evaluation
        .scorers()
        .iter()
        .map(|scorer| {
            (
                scorer.name.as_str(),
                scorer.flows.as_slice(),
                scorer.sample_percent,
            )
        })
        .collect();
    let code = ["code".to_string()];
    assert_eq!(
        scorers,
        [
            ("delivers-done-when", &code[..], 25),
            ("blockers-actionable", &code[..], 25),
        ]
    );
    // The starter documents Standing Roles without turning them on.
    assert!(!definition.agents.has_standing_roles());
}

#[tokio::test]
async fn a_definition_missing_a_file_it_names_does_not_read() {
    let mut without_flow = files();
    without_flow.remove("flows/plan.json");
    let mut without_agents = files();
    without_agents.remove("agents.json");
    let mut without_manifest = files();
    without_manifest.remove("factory.json");

    assert_eq!(
        read(without_flow).await,
        Err(DefinitionError::Missing("flows/plan.json".to_string()))
    );
    assert_eq!(
        read(without_agents).await,
        Err(DefinitionError::Missing("agents.json".to_string()))
    );
    assert_eq!(
        read(without_manifest).await,
        Err(DefinitionError::Missing("factory.json".to_string()))
    );
}

#[tokio::test]
async fn a_manifest_of_another_version_or_with_bad_flow_names_does_not_read() {
    assert_eq!(
        read(with(
            "factory.json",
            r#"{ "version": 2, "flows": ["reply"] }"#
        ))
        .await,
        Err(DefinitionError::Version(2))
    );
    assert_eq!(
        read(with("factory.json", r#"{ "version": 1, "flows": [] }"#)).await,
        Err(DefinitionError::FlowCount)
    );
    assert_eq!(
        read(with(
            "factory.json",
            r#"{ "version": 1, "flows": ["../code"] }"#
        ))
        .await,
        Err(DefinitionError::FlowName("../code".to_string()))
    );
    assert_eq!(
        read(with(
            "factory.json",
            r#"{ "version": 1, "flows": ["reply", "reply"] }"#
        ))
        .await,
        Err(DefinitionError::DuplicateFlow("reply".to_string()))
    );
    let unknown_field = read(with(
        "factory.json",
        r#"{ "version": 1, "flows": ["reply"], "token": "x" }"#,
    ))
    .await;
    assert!(
        matches!(&unknown_field, Err(DefinitionError::Unreadable { path, .. }) if path == "factory.json"),
        "{unknown_field:?}"
    );
}

#[tokio::test]
async fn a_flow_that_names_a_step_no_worker_serves_does_not_read() {
    let flow = r#"{ "entries": [{ "id": "deploy", "step": "ship_it", "on": {} }] }"#;

    let read = read(with("flows/reply.json", flow)).await;

    assert!(
        matches!(
            &read,
            Err(DefinitionError::Flow { path, error: FlowError::UnknownStep { .. } })
                if path == "flows/reply.json"
        ),
        "{read:?}"
    );
}

#[tokio::test]
async fn agents_that_leave_a_flows_role_unfilled_do_not_read() {
    let agents = r#"{
        "agents": { "primary": { "bot": "00000000-0000-0000-0000-00000000c1a0" } },
        "roles": { "planner": ["primary"] }
    }"#;

    assert_eq!(
        read(with("agents.json", agents)).await,
        Err(DefinitionError::Agents(AgentDefinitionError::UnfilledRole(
            Role::try_from("writer".to_string()).expect("a role")
        )))
    );
}

#[tokio::test]
async fn rules_the_factory_does_not_take_do_not_read() {
    assert_eq!(
        read(with("rules.json", r#"{ "controlFiles": ["/etc/passwd"] }"#)).await,
        Err(DefinitionError::ControlFiles(
            ControlFilesError::InvalidPattern("/etc/passwd".to_string())
        ))
    );
    assert_eq!(
        read(with(
            "rules.json",
            r#"{ "standingRules": { "plan": false, "blockers": false, "merge": false, "queueLimit": 0 } }"#
        ))
        .await,
        Err(DefinitionError::QueueLimit)
    );
    for budget in ["9999", "10000000001"] {
        assert_eq!(
            read(with(
                "rules.json",
                &format!(
                    r#"{{ "standingRules": {{ "plan": false, "blockers": false, "merge": false, "tokenBudget": {budget} }} }}"#
                )
            ))
            .await,
            Err(DefinitionError::TokenBudget),
            "a budget of {budget}"
        );
    }
    let too_many: Vec<String> = (0..=MAX_KNOWN_FLAKES).map(|job| format!("\"job {job}\"")).collect();
    for flakes in [
        r#"["e2e", "e2e"]"#.to_string(),
        r#"[" "]"#.to_string(),
        format!("[{}]", too_many.join(", ")),
    ] {
        assert_eq!(
            read(with("rules.json", &format!(r#"{{ "knownFlakes": {flakes} }}"#))).await,
            Err(DefinitionError::KnownFlakes),
            "known flakes {flakes}"
        );
    }
}

#[tokio::test]
async fn a_definition_names_its_known_flakes() {
    let named = read(with("rules.json", r#"{ "knownFlakes": ["e2e (macos)", "lint"] }"#))
        .await
        .expect("a definition with known flakes reads");
    assert_eq!(named.rules.known_flakes, ["e2e (macos)", "lint"]);
    let without = read(files()).await.expect("a definition without rules reads");
    assert!(without.rules.known_flakes.is_empty());
}

/// A token budget in range reads with the rules, and is kept on each Run
/// that starts under them.
#[tokio::test]
async fn a_token_budget_in_range_reads() {
    let definition = read(with(
        "rules.json",
        r#"{ "standingRules": { "plan": false, "blockers": true, "merge": false, "tokenBudget": 2000000 } }"#,
    ))
    .await
    .expect("a definition with a budget");
    assert_eq!(
        definition
            .rules
            .standing_rules
            .and_then(|rules| rules.token_budget),
        Some(2_000_000)
    );
}

/// `scorers.json` is refused when its Scorers do not hold together, when
/// they or its Experiments name a Flow or an Agent the Definition does not
/// have (a built-in Flow among them), and when it names a key twice, is in
/// another shape, or has a Scorer read what no judge is given.
#[tokio::test]
async fn scorers_that_do_not_fit_the_definition_do_not_read() {
    let unreadable = |error: Result<FactoryDefinition, DefinitionError>| match error {
        Err(DefinitionError::Unreadable { path, error }) => (path, error),
        other => panic!("not unreadable: {other:?}"),
    };

    let (path, error) = unreadable(
        read(with(
            "scorers.json",
            r#"{ "scorers": [
                { "name": "speed", "instructions": "Fast?", "samplePercent": 5 },
                { "name": "speed", "instructions": "Quick?", "samplePercent": 5 }
            ] }"#,
        ))
        .await,
    );
    assert_eq!(path, "scorers.json");
    assert!(error.contains("two scorers are named speed"), "{error}");

    assert_eq!(
        read(with(
            "scorers.json",
            r#"{ "scorers": [{ "name": "speed", "instructions": "Fast?", "flows": ["code"], "samplePercent": 5 }] }"#
        ))
        .await,
        Err(DefinitionError::Evaluation(
            EvaluationDefinitionError::UnknownFlow("code".to_string())
        ))
    );
    let Err(DefinitionError::Evaluation(EvaluationDefinitionError::UnknownAgent { agent, .. })) =
        read(with(
            "scorers.json",
            r#"{ "experiments": [{
                "name": "other-writer", "flow": "plan",
                "arms": [{ "name": "usual" }, { "name": "other", "agents": { "writer": "elsewhere" } }]
            }] }"#,
        ))
        .await
    else {
        panic!("an arm's Agent the definition does not have is refused");
    };
    assert_eq!(agent, "elsewhere");

    let (_, error) =
        unreadable(read(with("scorers.json", r#"{ "scorers": [], "scorers": [] }"#)).await);
    assert!(error.contains("appears twice"), "{error}");
    let (_, error) = unreadable(
        read(with(
            "scorers.json",
            r#"{ "scorers": [{ "name": "speed", "settings": { "weight": 2 } }] }"#,
        ))
        .await,
    );
    assert!(error.contains("settings"), "{error}");
    let (_, error) = unreadable(
        read(with(
            "scorers.json",
            r#"{ "scorers": [{ "name": "slop", "instructions": "Slop?", "samplePercent": 5, "reads": ["logs"] }] }"#,
        ))
        .await,
    );
    assert!(error.contains("unknown variant `logs`"), "{error}");
}

/// The improve Flow is the factory's own, which only a self-improvement
/// Run walks: a Definition names no Flow of its name.
#[tokio::test]
async fn a_definition_names_no_improve_flow() {
    let mut named = with(
        "factory.json",
        r#"{ "version": 1, "flows": ["reply", "improve"] }"#,
    );
    named.insert("flows/improve.json".to_string(), REPLY.to_string());
    assert_eq!(
        read(named).await,
        Err(DefinitionError::ReservedFlow("improve".to_string()))
    );
}

/// The benchmark Flow is the factory's own, which only a Benchmark's
/// trials walk: a Definition names no Flow of its name, and one with
/// Benchmarks has Agents for its Roles.
#[tokio::test]
async fn a_definition_keeps_to_the_benchmark_flow() {
    let mut named = with(
        "factory.json",
        r#"{ "version": 1, "flows": ["reply", "benchmark"] }"#,
    );
    named.insert("flows/benchmark.json".to_string(), REPLY.to_string());
    assert_eq!(
        read(named).await,
        Err(DefinitionError::ReservedFlow("benchmark".to_string()))
    );

    let benchmark = r#"{
        "benchmarks": [{
            "name": "core", "repository": "https://github.com/acme/widgets",
            "tasks": [{ "name": "typo", "request": "Fix the typo." }]
        }]
    }"#;
    let mut writer_only = with("scorers.json", benchmark);
    writer_only.insert(
        "factory.json".to_string(),
        r#"{ "version": 1, "flows": ["reply"] }"#.to_string(),
    );
    writer_only.insert(
        "agents.json".to_string(),
        r#"{
            "agents": { "primary": { "bot": "00000000-0000-0000-0000-00000000c1a0" } },
            "roles": { "writer": ["primary"] }
        }"#
        .to_string(),
    );
    assert!(matches!(
        read(writer_only).await,
        Err(DefinitionError::BenchmarkAgents(_))
    ));
    // Agents for its planner and writer, as these have, read.
    let read_benchmark = read(with("scorers.json", benchmark))
        .await
        .expect("a definition with a benchmark");
    assert_eq!(read_benchmark.evaluation.benchmarks()[0].name, "core");
}

#[tokio::test]
async fn a_file_holding_a_secret_does_not_read() {
    let agents = AGENTS.replace("large-model", concat!("ghp_", "0123456789abcdefABCDEF0123"));
    // Split so the repository's own secret scan does not read the fake key.
    let key = concat!(
        r#"{ "scorers": [{ "name": "speed", "instructions": "-----BEGIN "#,
        r#"PRIVATE KEY-----", "samplePercent": 0 }] }"#
    );

    assert_eq!(
        read(with("agents.json", &agents)).await,
        Err(DefinitionError::Secret {
            path: "agents.json".to_string(),
            kind: "ghp_",
        })
    );
    assert_eq!(
        read(with("scorers.json", key)).await,
        Err(DefinitionError::Secret {
            path: "scorers.json".to_string(),
            kind: "PRIVATE KEY",
        })
    );
    // A word that only begins like a token is not one.
    let instructions = AGENTS.replace("large-model", "use the task-runner skill");
    assert!(read(with("agents.json", &instructions)).await.is_ok());
}

/// A Definition of one Flow, `ship`, of `entries`, its Agents filling no
/// Role it needs.
fn shipping(entries: serde_json::Value) -> BTreeMap<String, String> {
    let flow = serde_json::json!({ "repository": true, "entries": entries });
    let mut files = with("factory.json", r#"{ "version": 1, "flows": ["ship"] }"#);
    files.insert("flows/ship.json".to_string(), flow.to_string());
    files
}

fn end(outcome: &str) -> serde_json::Value {
    serde_json::json!([{ "to": { "kind": "end", "outcome": outcome } }])
}

fn to(entry: &str) -> serde_json::Value {
    serde_json::json!([{ "to": { "kind": "entry", "entry": entry } }])
}

/// A merge Gate asking to merge, with every outcome of `ready_to_merge`
/// routed, and `approve` going to `approved`.
fn merge_gate(id: &str, kind: &str, approved: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "step": "ready_to_merge",
        "gate": { "kind": kind, "question": "Merge?", "answers": ["approve", "revise"] },
        "on": {
            "approve": approved,
            "revise": end("manual"),
            "changed": end("manual"),
            "merged": end("success"),
            "closed": end("manual")
        }
    })
}

fn merge_step() -> serde_json::Value {
    serde_json::json!({
        "id": "merge",
        "step": "merge",
        "on": {
            "merged": end("success"),
            "changed": end("manual"),
            "refused": end("manual"),
            "held": end("manual"),
            "closed": end("manual")
        }
    })
}

fn flow_error(read: Result<FactoryDefinition, DefinitionError>) -> FlowError {
    match read {
        Err(DefinitionError::Flow { error, .. }) => error,
        other => panic!("expected a flow error, got {other:?}"),
    }
}

/// A merge goes through a merge Gate's approval: a Gate on the merge's
/// entries of another kind, one named `plan` whose rule or Orchestrator
/// approves plans, and a way into `merge` that skips the Gate do not read;
/// the built-in way does.
#[tokio::test]
async fn a_merge_needs_a_merge_gate_s_approval() {
    let entry = |id: &str| EntryId::try_from(id.to_string()).expect("valid");

    let guarded = read(shipping(serde_json::json!([
        merge_gate("merge-gate", "merge", to("merge")),
        merge_step()
    ])))
    .await;
    assert!(guarded.is_ok(), "{guarded:?}");

    // A merge's Gate labelled `plan`, in an entry named `plan`.
    let relabelled = read(shipping(serde_json::json!([
        merge_gate("plan", "plan", to("merge")),
        merge_step()
    ])))
    .await;
    assert_eq!(
        flow_error(relabelled),
        FlowError::MergeGateKind(entry("plan"))
    );

    // A merge Gate on an entry that does not ask to merge.
    let mut elsewhere = merge_gate("merge-gate", "merge", to("merge"));
    elsewhere["step"] = serde_json::json!("reply");
    let elsewhere = read(shipping(serde_json::json!([elsewhere, merge_step()]))).await;
    assert_eq!(
        flow_error(elsewhere),
        FlowError::MergeGateKind(entry("merge-gate"))
    );

    // CI green straight to the merge, with no Gate.
    let straight = read(shipping(serde_json::json!([
        { "id": "ci", "step": "ci", "on": { "green": to("merge"), "red": end("manual") } },
        merge_step()
    ])))
    .await;
    assert_eq!(
        flow_error(straight),
        FlowError::UnguardedMerge {
            from: entry("ci"),
            to: entry("merge"),
        }
    );

    // The Gate's `revise` going to the merge.
    let mut revised = merge_gate("merge-gate", "merge", end("manual"));
    revised["on"]["revise"] = to("merge");
    let revised = read(shipping(serde_json::json!([revised, merge_step()]))).await;
    assert_eq!(
        flow_error(revised),
        FlowError::UnguardedMerge {
            from: entry("merge-gate"),
            to: entry("merge"),
        }
    );

    // An outcome the Gate's entry leaves unrouted falls down into the merge.
    let mut unrouted = merge_gate("merge-gate", "merge", to("merge"));
    unrouted["on"]
        .as_object_mut()
        .expect("routes")
        .remove("closed");
    let unrouted = read(shipping(serde_json::json!([unrouted, merge_step()]))).await;
    assert_eq!(
        flow_error(unrouted),
        FlowError::UnguardedMerge {
            from: entry("merge-gate"),
            to: entry("merge"),
        }
    );

    // A merge that waited its turn goes on waiting with its approval: its
    // `queued` back to itself reads, but into another merge, or any other
    // outcome back to itself, does not.
    let mut waiting = merge_step();
    waiting["on"]["queued"] = to("merge");
    let waiting = read(shipping(serde_json::json!([
        merge_gate("merge-gate", "merge", to("merge")),
        waiting
    ])))
    .await;
    assert!(waiting.is_ok(), "{waiting:?}");

    let mut elsewhere = merge_step();
    elsewhere["on"]["queued"] = to("other-merge");
    let mut other = merge_step();
    other["id"] = serde_json::json!("other-merge");
    let elsewhere = read(shipping(serde_json::json!([
        merge_gate("merge-gate", "merge", to("merge")),
        elsewhere,
        other
    ])))
    .await;
    assert_eq!(
        flow_error(elsewhere),
        FlowError::UnguardedMerge {
            from: entry("merge"),
            to: entry("other-merge"),
        }
    );

    let mut again = merge_step();
    again["on"]["refused"] = to("merge");
    let again = read(shipping(serde_json::json!([
        merge_gate("merge-gate", "merge", to("merge")),
        again
    ])))
    .await;
    assert_eq!(
        flow_error(again),
        FlowError::UnguardedMerge {
            from: entry("merge"),
            to: entry("merge"),
        }
    );

    // A Run that would start at the merge.
    let first = read(shipping(serde_json::json!([merge_step()]))).await;
    assert_eq!(
        flow_error(first),
        FlowError::UnguardedMerge {
            from: entry("merge"),
            to: entry("merge"),
        }
    );
}

/// A token hidden behind JSON escapes, or under a newer prefix, is still
/// a secret; a word that only ends like a prefix is not.
#[tokio::test]
async fn an_escaped_or_newer_token_is_a_secret() {
    let escaped = AGENTS.replace("large-model", r"ghp\u005f0123456789abcdefABCDEF0123");
    let gitlab = AGENTS.replace("large-model", concat!("glpat-", "0123456789abcdefABCD"));
    let words = AGENTS.replace("large-model", "a task-runner-for-every-long-check");

    assert_eq!(
        read(with("agents.json", &escaped)).await,
        Err(DefinitionError::Secret {
            path: "agents.json".to_string(),
            kind: "ghp_",
        })
    );
    assert_eq!(
        read(with("agents.json", &gitlab)).await,
        Err(DefinitionError::Secret {
            path: "agents.json".to_string(),
            kind: "glpat-",
        })
    );
    assert!(read(with("agents.json", &words)).await.is_ok());
}

/// A team that copies the template and leaves its placeholder bot ids
/// can't save it as its Definition or start a Run of it: the validator,
/// the team setting and a Run all read it this way.
#[tokio::test]
async fn the_starter_definition_s_placeholder_bots_do_not_read() {
    let error = match read_definition(&template(), built_in_step_use).await {
        Err(DefinitionReadError::Invalid(error)) => error,
        Err(DefinitionReadError::Files(error)) => panic!("the starter's files: {error}"),
        Ok(_) => panic!("the starter's placeholder bots must not read"),
    };

    assert_eq!(
        error.to_string(),
        "agents.json: agents primary, backup have the template's placeholder bot ids: \
         replace them with the ids of Macro agents each member who starts runs may use"
    );
}

/// An Agent whose bot is all zeros but its last two digits names no
/// agent, and the error names `agents.json` and the Agent; one past them
/// is an id like any other.
#[tokio::test]
async fn a_placeholder_bot_id_does_not_read() {
    let agents = |bot: &str| AGENTS.replace("00000000-0000-0000-0000-00000000c1a0", bot);
    let primary = AgentName::try_from("primary".to_string()).expect("an agent");

    for bot in [
        "00000000-0000-0000-0000-000000000000",
        "00000000-0000-0000-0000-000000000001",
        "00000000-0000-0000-0000-0000000000ff",
    ] {
        let error = read(with("agents.json", &agents(bot)))
            .await
            .expect_err("a placeholder bot");
        assert_eq!(
            error,
            DefinitionError::Agents(AgentDefinitionError::PlaceholderBots(vec![primary.clone()])),
            "{bot}"
        );
        assert_eq!(
            error.to_string(),
            "agents.json: agent primary has the template's placeholder bot id: replace it \
             with the id of a Macro agent each member who starts runs may use",
        );
    }

    let real = "00000000-0000-0000-0000-000000000100";
    assert!(read(with("agents.json", &agents(real))).await.is_ok());
    let random = "6f1c2a9e-4b7d-4e2a-9c3f-0d8e5b1a7c42";
    assert!(read(with("agents.json", &agents(random))).await.is_ok());
}

/// Every Agent that keeps a placeholder bot id is named, in the order
/// `agents.json` lists them rather than by name.
#[tokio::test]
async fn every_placeholder_bot_is_named_in_the_file_s_order() {
    let agents = r#"{
        "agents": {
            "primary": { "bot": "00000000-0000-0000-0000-000000000001" },
            "spare": { "bot": "00000000-0000-0000-0000-00000000c0d1" },
            "backup": { "bot": "00000000-0000-0000-0000-000000000002" }
        },
        "roles": { "planner": ["primary"], "writer": ["primary"] }
    }"#;
    let name = |name: &str| AgentName::try_from(name.to_string()).expect("an agent");

    assert_eq!(
        read(with("agents.json", agents)).await,
        Err(DefinitionError::Agents(
            AgentDefinitionError::PlaceholderBots(vec![name("primary"), name("backup")])
        ))
    );
}

/// An Agent on Codex's cloud agent does not read, named in the file's
/// order; one on a machine's agent with a Codex model reads like any other.
#[tokio::test]
async fn an_agent_on_the_codex_cloud_agent_does_not_read() {
    let agents = r#"{
        "agents": {
            "primary": { "bot": "00000000-0000-0000-0000-00000000c1a0" },
            "zeta": { "bot": "00000000-0000-0000-0000-00000000c0de", "model": "gpt-5.5" },
            "alpha": { "bot": "00000000-0000-0000-0000-00000000c0de" }
        },
        "roles": { "planner": ["zeta", "primary"], "writer": ["primary", "alpha"] }
    }"#;
    let name = |name: &str| AgentName::try_from(name.to_string()).expect("an agent");

    assert_eq!(
        read(with("agents.json", agents)).await,
        Err(DefinitionError::Agents(AgentDefinitionError::CloudCodex(
            vec![name("zeta"), name("alpha")]
        )))
    );
    let on_a_machine = agents.replace(
        "00000000-0000-0000-0000-00000000c0de",
        "6f1c2a9e-4b7d-4e2a-9c3f-0d8e5b1a7c42",
    );
    assert!(read(with("agents.json", &on_a_machine)).await.is_ok());
}

/// An Agent on Codex's cloud agent that only a Standing Role lists reads:
/// it takes no Step, so nothing waits on it to hand back.
#[tokio::test]
async fn an_agent_on_the_codex_cloud_agent_reads_in_a_standing_role_only() {
    let agents = r#"{
        "agents": {
            "primary": { "bot": "00000000-0000-0000-0000-00000000c1a0" },
            "backup": { "bot": "00000000-0000-0000-0000-00000000c0de" }
        },
        "roles": { "planner": ["primary"], "writer": ["primary"] },
        "standing": {
            "intake": { "acts": ["start", "link"], "agents": ["backup"] }
        }
    }"#;

    let definition = read(with("agents.json", agents))
        .await
        .expect("a cloud agent in a standing role only reads");
    assert!(definition.agents.has_standing_roles());
    assert!(definition.agents.cloud_codex_agents().is_empty());
}

/// An arm of `scorers.json` that gives a Role an Agent on Codex's cloud
/// agent does not read, though only a Standing Role lists the Agent: the
/// arm gives it the Role's Step, which it could never hand back.
#[tokio::test]
async fn an_arm_choosing_an_agent_on_the_codex_cloud_agent_does_not_read() {
    let mut files = with(
        "agents.json",
        r#"{
            "agents": {
                "primary": { "bot": "00000000-0000-0000-0000-00000000c1a0" },
                "backup": { "bot": "00000000-0000-0000-0000-00000000c0de" }
            },
            "roles": { "planner": ["primary"], "writer": ["primary"] },
            "standing": {
                "intake": { "acts": ["start", "link"], "agents": ["backup"] }
            }
        }"#,
    );
    files.insert(
        "scorers.json".to_string(),
        r#"{ "experiments": [{
            "name": "other-writer", "flow": "plan",
            "arms": [{ "name": "usual" }, { "name": "other", "agents": { "writer": "backup" } }]
        }] }"#
            .to_string(),
    );

    let Err(DefinitionError::Evaluation(error @ EvaluationDefinitionError::CloudCodexAgent { .. })) =
        read(files).await
    else {
        panic!("an arm choosing an Agent on the cloud agent is refused");
    };
    assert_eq!(
        error.to_string(),
        "arm other of experiment other-writer gives role writer agent backup, which runs on \
         the Codex cloud agent, whose sessions cannot hand back a step's result: run Codex \
         with macrod on a machine (`macrod service --agent codex`) and use the bot id of \
         that machine's agent"
    );
}

/// The starter's Control Files are the factory's default and its own
/// files, so a Run that changes its rules waits for a person.
#[tokio::test]
async fn the_starter_s_control_files_keep_the_default_and_its_own_files() {
    let definition = read_definition(&Customized(template()), built_in_step_use)
        .await
        .expect("the starter definition reads once its bots are replaced");
    let patterns = definition.rules.control_files.expect("its own list");

    for pattern in ControlFiles::default_patterns()
        .iter()
        .map(String::as_str)
        .chain(DEFINITION_FILE_PATTERNS)
    {
        assert!(patterns.iter().any(|kept| kept == pattern), "{pattern}");
    }
}

/// A merge Gate opens only after `done`, the head whose checks passed: one
/// after `changed` would ask about a red head and let `done` fall down
/// into the merge. No skip, exhausted visit or `next` leads into it either.
#[tokio::test]
async fn nothing_but_a_merge_gate_s_approval_falls_into_the_merge() {
    let entry = |id: &str| EntryId::try_from(id.to_string()).expect("valid");
    let unguarded = || FlowError::UnguardedMerge {
        from: entry("merge-gate"),
        to: entry("merge"),
    };

    let mut after_changed = merge_gate("merge-gate", "merge", to("merge"));
    after_changed["gate"]["after"] = serde_json::json!("changed");
    let after_changed = read(shipping(serde_json::json!([after_changed, merge_step()]))).await;
    assert_eq!(
        flow_error(after_changed),
        FlowError::MergeGateAfter(entry("merge-gate"))
    );

    let mut skipped = merge_gate("merge-gate", "merge", to("merge"));
    skipped["when"] = serde_json::json!([{ "fact": "ready", "equals": true }]);
    let flow = serde_json::json!({
        "repository": true,
        "facts": { "ready": false },
        "entries": [skipped, merge_step()]
    });
    let mut files = shipping(serde_json::json!([]));
    files.insert("flows/ship.json".to_string(), flow.to_string());
    assert_eq!(flow_error(read(files).await), unguarded());

    let mut exhausted = merge_gate("merge-gate", "merge", to("merge"));
    exhausted["max_visits"] = serde_json::json!(2);
    exhausted["on_exhausted"] = to("merge");
    let exhausted = read(shipping(serde_json::json!([exhausted, merge_step()]))).await;
    assert_eq!(flow_error(exhausted), unguarded());

    let mut next = merge_gate("merge-gate", "merge", end("manual"));
    next["next"] = serde_json::json!({ "kind": "entry", "entry": "merge" });
    let next = read(shipping(serde_json::json!([next, merge_step()]))).await;
    assert_eq!(flow_error(next), unguarded());
}

/// A Definition's Flow ends at the merge: the Steps that ran after it are
/// served by no worker for it.
#[tokio::test]
async fn a_definition_s_flow_runs_no_post_merge_step() {
    let deploy = r#"{ "entries": [{ "id": "deploy", "step": "deploy", "on": {} }] }"#;
    let verify =
        r#"{ "entries": [{ "id": "verify", "step": "verify", "role": "verifier", "on": {} }] }"#;

    let deployed = read(with("flows/reply.json", deploy)).await;
    let verified = read(with("flows/reply.json", verify)).await;

    assert!(
        matches!(
            &deployed,
            Err(DefinitionError::Flow {
                error: FlowError::UnknownStep { .. },
                ..
            })
        ),
        "{deployed:?}"
    );
    assert!(
        matches!(
            &verified,
            Err(DefinitionError::Flow {
                error: FlowError::UnknownStep { .. },
                ..
            })
        ),
        "{verified:?}"
    );
}

/// A token glued to what precedes it, as in a URL's query, is a secret all
/// the same; only `sk-` needs the start of a word, so `task-` is none.
#[tokio::test]
async fn a_token_glued_to_text_is_a_secret() {
    let glued = concat!(
        r#"{ "scorers": [{ "name": "s", "settings": { "hook": "https://example.com/hook?auth=Bearer%20ghp_"#,
        r#"abcdefghijklmnopqrstuvwxyz0123456789" } }] }"#
    );
    let stripe = AGENTS.replace("large-model", concat!("rk_live_", "0123456789abcdefABCDEF"));
    let npm = AGENTS.replace("large-model", "npm_0123456789abcdefABCDEF0123");

    assert_eq!(
        read(with("scorers.json", glued)).await,
        Err(DefinitionError::Secret {
            path: "scorers.json".to_string(),
            kind: "ghp_",
        })
    );
    assert_eq!(
        read(with("agents.json", &stripe)).await,
        Err(DefinitionError::Secret {
            path: "agents.json".to_string(),
            kind: "rk_live_",
        })
    );
    assert_eq!(
        read(with("agents.json", &npm)).await,
        Err(DefinitionError::Secret {
            path: "agents.json".to_string(),
            kind: "npm_",
        })
    );
}

/// A Definition's Control Files leave room for its own files, which a Run
/// on its repository adds: a list one longer does not read.
#[tokio::test]
async fn a_definition_s_control_files_leave_room_for_its_own() {
    let list = |count: usize| {
        let patterns: Vec<String> = (0..count).map(|at| format!("docs/{at}/**")).collect();
        serde_json::json!({ "controlFiles": patterns }).to_string()
    };

    let room = read(with(
        "rules.json",
        &list(super::MAX_DEFINITION_CONTROL_FILES),
    ))
    .await;
    let full = read(with(
        "rules.json",
        &list(super::MAX_DEFINITION_CONTROL_FILES + 1),
    ))
    .await;

    let over = read(with(
        "rules.json",
        &list(crate::domain::control_files::MAX_PATTERNS + 1),
    ))
    .await;

    assert!(room.is_ok(), "{room:?}");
    assert_eq!(full, Err(DefinitionError::TooManyControlFiles));
    // A list past what any Run keeps says the same, not the generic limit.
    assert_eq!(over, Err(DefinitionError::TooManyControlFiles));
}

/// A key named twice in an object does not read: serde would take the
/// last, which a person reading the file may not.
#[tokio::test]
async fn a_key_named_twice_does_not_read() {
    let manifest = r#"{ "version": 1, "flows": ["reply", "plan"], "flows": ["reply"] }"#;

    let read = read(with("factory.json", manifest)).await;

    assert!(
        matches!(&read, Err(DefinitionError::Unreadable { path, error }) if path == "factory.json" && error.contains("twice")),
        "{read:?}"
    );
}

/// A Step the worker does itself names no Role or reviewers.
#[tokio::test]
async fn a_work_step_names_no_role() {
    let mut merge = merge_step();
    merge["role"] = serde_json::json!("writer");

    let read = read(shipping(serde_json::json!([
        merge_gate("merge-gate", "merge", to("merge")),
        merge
    ])))
    .await;

    assert_eq!(
        flow_error(read),
        FlowError::RoleOnWork(EntryId::try_from("merge".to_string()).expect("valid"))
    );
}

/// Slack, Google, webhook and cloud secrets, and a password in a URL, do
/// not read; npm's settings and a public certificate do.
#[tokio::test]
async fn more_secret_shapes_do_not_read_and_public_text_does() {
    let kind = |value: &str| {
        let agents = AGENTS.replace("large-model", value);
        async move {
            match read(with("agents.json", &agents)).await {
                Err(DefinitionError::Secret { kind, .. }) => Some(kind),
                _ => None,
            }
        }
    };

    // Split so the repository's own secret scan does not read the fake token.
    assert_eq!(
        kind(concat!("xoxs", "-0123456789abcdefABCDEF")).await,
        Some("xoxs-")
    );
    assert_eq!(
        kind("https://hooks.slack.com/services/T000/B000/XXXX").await,
        Some("hooks.slack.com/services/")
    );
    assert_eq!(kind("ya29.0123456789abcdefABCDEF").await, Some("ya29."));
    assert_eq!(kind("whsec_0123456789abcdefABCDEF").await, Some("whsec_"));
    assert_eq!(
        kind("AWS_SECRET_ACCESS_KEY=abc").await,
        Some("aws_secret_access_key")
    );
    assert_eq!(kind("AccountKey=abc;").await, Some("accountkey="));
    assert_eq!(
        kind("postgres://admin:hunter2@db.test/app").await,
        Some("://user:password@")
    );
    assert_eq!(kind("npm_config_registry_settings_value").await, None);
    assert_eq!(kind("-----BEGIN CERTIFICATE-----").await, None);
    assert_eq!(kind("-----BEGIN PUBLIC KEY-----").await, None);
    assert_eq!(
        kind("-----BEGIN PGP PRIVATE KEY BLOCK-----").await,
        Some("PRIVATE KEY")
    );
    assert_eq!(
        kind("redis://:hunter2@cache.test:6379").await,
        Some("://user:password@")
    );
    assert_eq!(kind("xoxe-0123456789abcdefABCDEF").await, Some("xoxe-"));
    assert_eq!(kind("https://github.com/macro/app").await, None);
}

/// `agents.json` with the Definition's Agents and `standing` as its
/// Standing Roles.
fn standing_agents(standing: &str) -> String {
    format!(
        r#"{{
            "agents": {{
                "primary": {{ "bot": "00000000-0000-0000-0000-00000000c1a0" }},
                "backup": {{ "bot": "00000000-0000-0000-0000-00000000c0d1" }}
            }},
            "roles": {{ "planner": ["primary", "backup"], "writer": ["primary"] }},
            "standing": {standing}
        }}"#
    )
}

/// A Definition reads with one Agent in every Standing Role, and with an
/// Agent per Standing Role.
#[tokio::test]
async fn a_definition_reads_with_standing_roles() {
    let one_agent = standing_agents(
        r#"{
            "intake": { "acts": ["start", "link"], "agents": ["primary"] },
            "gatekeeper": { "acts": ["answer_plan", "answer_blockers"], "agents": ["primary"] },
            "operator": { "acts": ["cancel"], "agents": ["primary"] }
        }"#,
    );
    let own_agents = standing_agents(
        r#"{
            "intake": { "description": "Starts runs", "acts": ["start", "link"], "agents": ["backup"] },
            "gatekeeper": { "acts": ["answer_plan"], "agents": ["primary", "backup"] }
        }"#,
    );

    for agents in [one_agent, own_agents] {
        let definition = read(with("agents.json", &agents))
            .await
            .expect("a definition with standing roles");
        assert!(definition.agents.has_standing_roles());
    }
}

/// The validator refuses an unknown act, any merge, an Agent the
/// Definition does not have, a Standing Role no Agent fills, and a
/// Standing Role named twice, naming the file, the Standing Role and the
/// problem.
#[tokio::test]
async fn standing_roles_that_do_not_hold_together_do_not_read() {
    let refused = [
        (
            r#"{ "intake": { "acts": ["start", "approve_everything"], "agents": ["primary"] } }"#,
            r#"standing role intake names act "approve_everything", which is not one of"#,
        ),
        (
            r#"{ "release": { "acts": ["merge"], "agents": ["primary"] } }"#,
            r#"standing role release names act "merge": no standing role merges"#,
        ),
        (
            r#"{ "gatekeeper": { "acts": ["answer_merge"], "agents": ["primary"] } }"#,
            r#"standing role gatekeeper names act "answer_merge": no standing role merges"#,
        ),
        (
            r#"{ "intake": { "acts": ["start"], "agents": ["stranger"] } }"#,
            "standing role intake lists agent stranger, which the definition does not have",
        ),
        (
            r#"{ "intake": { "acts": ["start"], "agents": [] } }"#,
            "standing role intake lists no agent",
        ),
        (
            r#"{
                "intake": { "acts": ["start"], "agents": ["primary"] },
                "intake": { "acts": ["start", "cancel"], "agents": ["backup"] }
            }"#,
            r#"key "intake" appears twice"#,
        ),
    ];

    for (standing, why) in refused {
        let read = read(with("agents.json", &standing_agents(standing))).await;
        assert!(
            matches!(&read, Err(DefinitionError::Unreadable { path, error }) if path == "agents.json" && error.contains(why)),
            "{why}: {read:?}"
        );
    }
}

/// `agents.json` with the Definition's Agents, a `scribe` Agent only a
/// Standing Role lists (as a bot that answers Gates), and `rules` as its
/// Agent Rules.
fn ruled_agents(rules: &str) -> String {
    format!(
        r#"{{
            "agents": {{
                "primary": {{ "bot": "00000000-0000-0000-0000-00000000c1a0" }},
                "backup": {{ "bot": "00000000-0000-0000-0000-00000000c0d1" }},
                "scribe": {{ "bot": "00000000-0000-0000-0000-00000000d0c5" }}
            }},
            "roles": {{ "planner": ["primary", "backup"], "writer": ["primary", "backup"] }},
            "standing": {{ "intake": {{ "acts": ["start"], "agents": ["scribe"] }} }},
            "rules": {rules}
        }}"#
    )
}

/// A Definition reads with Agent Rules on a tag, a repository or both,
/// each choosing one of its Role's own Agents.
#[tokio::test]
async fn a_definition_reads_with_agent_rules() {
    let agents = ruled_agents(
        r#"[
            { "name": "docs", "role": "writer", "tag": "docs", "agent": "backup" },
            { "name": "site", "role": "planner", "repository": "acme/site", "agent": "backup" },
            { "name": "site-docs", "role": "writer", "tag": "Docs", "repository": "acme/site", "agent": "primary" }
        ]"#,
    );

    let definition = read(with("agents.json", &agents))
        .await
        .expect("a definition with agent rules");

    let names: Vec<&str> = definition
        .agents
        .rules()
        .iter()
        .map(|rule| rule.name.as_str())
        .collect();
    assert_eq!(names, ["docs", "site", "site-docs"]);
}

/// The validator refuses an Agent Rule whose Agent the Definition does not
/// have (as an arm's, through `check_choices`), one whose Agent its Role
/// does not list (one only a Standing Role lists among them), one for a
/// Role it does not fill, one that matches nothing, one whose repository is
/// not `owner/name` as a Run keeps it, one with an empty tag, one an
/// earlier rule shadows, and two rules of one name, naming the file, the
/// rule and the problem.
#[tokio::test]
async fn agent_rules_that_do_not_hold_together_do_not_read() {
    let unknown_agent = read(with(
        "agents.json",
        &ruled_agents(r#"[{ "name": "docs", "role": "writer", "tag": "docs", "agent": "ghost" }]"#),
    ))
    .await;
    assert!(
        matches!(
            &unknown_agent,
            Err(DefinitionError::AgentRule(EvaluationDefinitionError::UnknownAgent { whose, agent, .. }))
                if whose == "agent rule docs" && agent == "ghost"
        ),
        "{unknown_agent:?}"
    );
    assert_eq!(
        unknown_agent.expect_err("refused").to_string(),
        "agents.json: agent rule docs gives role writer agent ghost, which the agent definition \
         does not have"
    );

    let refused = [
        (
            r#"[{ "name": "docs", "role": "writer", "tag": "docs", "agent": "scribe" }]"#,
            "agent rule docs chooses agent scribe, which role writer does not list",
        ),
        (
            r#"[{ "name": "plans", "role": "planner", "tag": "docs", "agent": "scribe" }]"#,
            "agent rule plans chooses agent scribe, which role planner does not list",
        ),
        (
            r#"[{ "name": "site", "role": "writer", "repository": "acme/site.git", "agent": "backup" }]"#,
            "agent rule site must name its repository as owner/name",
        ),
        (
            r#"[{ "name": "site", "role": "writer", "repository": "acme/..", "agent": "backup" }]"#,
            "agent rule site must name its repository as owner/name",
        ),
        (
            r#"[{ "name": "site", "role": "writer", "repository": "./site", "agent": "backup" }]"#,
            "agent rule site must name its repository as owner/name",
        ),
        (
            r#"[{ "name": "site", "role": "writer", "repository": "acme/site/", "agent": "backup" }]"#,
            "agent rule site must name its repository as owner/name",
        ),
        (
            r#"[
                { "name": "docs", "role": "writer", "tag": "docs", "repository": "acme/site", "agent": "backup" },
                { "name": "docs-again", "role": "writer", "tag": " Docs ", "repository": "Acme/Site", "agent": "primary" }
            ]"#,
            "agent rule docs-again matches the same runs for the same role as agent rule docs",
        ),
        (
            r#"[{ "name": "docs", "role": "publisher", "tag": "docs", "agent": "backup" }]"#,
            "agent rule docs chooses for role publisher, which the definition does not fill",
        ),
        (
            r#"[{ "name": "docs", "role": "writer", "agent": "backup" }]"#,
            "agent rule docs must name a tag, a repository, or both",
        ),
        (
            r#"[{ "name": "site", "role": "writer", "repository": "https://github.com/acme/site", "agent": "backup" }]"#,
            "agent rule site must name its repository as owner/name",
        ),
        (
            r#"[{ "name": "docs", "role": "writer", "tag": " ", "agent": "backup" }]"#,
            "agent rule docs must name a tag of 1 to 64 characters",
        ),
        (
            r#"[{ "name": "Docs Rule", "role": "writer", "tag": "docs", "agent": "backup" }]"#,
            r#"agent rule name "Docs Rule" must be 1 to 64"#,
        ),
        (
            r#"[
                { "name": "docs", "role": "writer", "tag": "docs", "agent": "backup" },
                { "name": "docs", "role": "planner", "tag": "docs", "agent": "backup" }
            ]"#,
            "two agent rules are named docs",
        ),
        (
            r#"[{ "name": "docs", "role": "writer", "tag": "docs", "agent": "backup", "model": "small" }]"#,
            "unknown field `model`",
        ),
    ];

    for (rules, why) in refused {
        let read = read(with("agents.json", &ruled_agents(rules))).await;
        assert!(
            matches!(&read, Err(DefinitionError::Unreadable { path, error }) if path == "agents.json" && error.contains(why)),
            "{why}: {read:?}"
        );
    }
}

/// Rules that differ in their Role or in what they match read: only one
/// that an earlier rule always beats is refused.
#[tokio::test]
async fn agent_rules_that_differ_in_role_or_match_read() {
    let agents = ruled_agents(
        r#"[
            { "name": "docs", "role": "writer", "tag": "docs", "agent": "backup" },
            { "name": "docs-plans", "role": "planner", "tag": "docs", "agent": "backup" },
            { "name": "site-docs", "role": "writer", "tag": "docs", "repository": "acme/site", "agent": "primary" },
            { "name": "site", "role": "writer", "repository": "acme/site", "agent": "backup" }
        ]"#,
    );

    let definition = read(with("agents.json", &agents))
        .await
        .expect("rules that differ read");
    assert_eq!(definition.agents.rules().len(), 4);
}
