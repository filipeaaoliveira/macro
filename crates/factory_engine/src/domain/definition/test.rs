use std::collections::BTreeMap;
use std::convert::Infallible;

use factory_records::domain::models::{AgentName, EntryId, Role};
use factory_records::domain::standing_rules::OrchestratorDefinition;

use super::{
    DefinitionError, DefinitionFiles, DefinitionReadError, FactoryDefinition, read_definition,
};
use crate::domain::agents::AgentDefinitionError;
use crate::domain::built_in_steps::built_in_step_use;
use crate::domain::control_files::ControlFilesError;
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
        r#"{ "scorers": [{ "name": "tests-pass", "settings": { "weight": 2 } }] }"#.to_string(),
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
    assert_eq!(definition.scorers[0].name, "tests-pass");
    assert_eq!(definition.scorers[0].settings["weight"], 2);
}

#[tokio::test]
async fn rules_and_scorers_left_out_take_the_defaults() {
    let definition = read(files()).await.expect("a definition");

    assert_eq!(definition.rules.standing_rules, None);
    assert_eq!(definition.rules.control_files, None);
    assert!(definition.scorers.is_empty());
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

#[tokio::test]
async fn scorers_need_distinct_names() {
    assert_eq!(
        read(with(
            "scorers.json",
            r#"{ "scorers": [{ "name": "speed" }, { "name": "speed" }] }"#
        ))
        .await,
        Err(DefinitionError::DuplicateScorer("speed".to_string()))
    );
    assert_eq!(
        read(with(
            "scorers.json",
            r#"{ "scorers": [{ "name": "Speed" }] }"#
        ))
        .await,
        Err(DefinitionError::ScorerName("Speed".to_string()))
    );
}

#[tokio::test]
async fn a_file_holding_a_secret_does_not_read() {
    let agents = AGENTS.replace("large-model", concat!("ghp_", "0123456789abcdefABCDEF0123"));
    let key = r#"{ "scorers": [{ "name": "speed", "settings": { "key": "-----BEGIN PRIVATE KEY-----" } }] }"#;

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
            kind: "-----BEGIN",
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

