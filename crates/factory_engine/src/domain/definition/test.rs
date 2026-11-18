use std::collections::BTreeMap;
use std::convert::Infallible;

use factory_records::domain::models::{AgentName, EntryId, Role};
use factory_records::domain::standing_rules::OrchestratorDefinition;

use super::{
    DEFINITION_FILE_PATTERNS, DefinitionError, DefinitionFiles, DefinitionReadError,
    FactoryDefinition, read_definition,
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
        "backup": { "bot": "00000000-0000-0000-0000-00000000c0de" }
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

#[tokio::test]
async fn the_starter_definition_reads() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/definition_template");
    let definition = read_definition(&LocalDefinitionFiles::new(root), built_in_step_use)
        .await
        .expect("the starter definition reads");

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
    // Every judgment is a model call: the starter's Scorers judge no Run.
    let scorers = definition.evaluation.scorers();
    assert!(!scorers.is_empty());
    assert!(scorers.iter().all(|scorer| scorer.sample_percent == 0));
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
}

/// `scorers.json` is refused when its Scorers do not hold together, when
/// they or its Experiments name a Flow or an Agent the Definition does not
/// have (a built-in Flow among them), and when it names a key twice or is
/// in another shape.
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

/// The starter's Control Files are the factory's default and its own
/// files, so a Run that changes its rules waits for a person.
#[tokio::test]
async fn the_starter_s_control_files_keep_the_default_and_its_own_files() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/definition_template");
    let definition = read_definition(&LocalDefinitionFiles::new(root), built_in_step_use)
        .await
        .expect("the starter definition reads");
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
