use std::collections::BTreeMap;
use std::convert::Infallible;

use super::{
    DefinitionChange, DefinitionChangeError, DefinitionChangeReadError, MAX_CHANGED_FILES,
};
use crate::domain::built_in_steps::built_in_step_use;
use crate::domain::definition::{DefinitionError, DefinitionFiles, FactoryDefinition};

/// A Definition's files, by path.
struct Files(BTreeMap<String, String>);

impl DefinitionFiles for Files {
    type Err = Infallible;

    async fn read(&self, path: &str) -> Result<Option<String>, Infallible> {
        Ok(self.0.get(path).cloned())
    }
}

const MANIFEST: &str = r#"{ "version": 1, "description": "Ours.", "flows": ["reply"] }"#;

const AGENTS: &str = r#"{
    "agents": { "primary": { "bot": "00000000-0000-0000-0000-00000000c1a0" } },
    "roles": { "planner": ["primary"], "writer": ["primary"] }
}"#;

const REPLY: &str = include_str!("../../../../flows/reply.json");

const PLAN: &str = include_str!("../../../../flows/plan.json");

/// A Definition of the `reply` Flow, as its default branch holds it.
fn base() -> Files {
    Files(BTreeMap::from([
        ("factory.json".to_string(), MANIFEST.to_string()),
        ("flows/reply.json".to_string(), REPLY.to_string()),
        ("agents.json".to_string(), AGENTS.to_string()),
    ]))
}

fn change(files: &[(&str, &str)]) -> DefinitionChange {
    DefinitionChange::new(
        "Add the plan flow".to_string(),
        None,
        files
            .iter()
            .map(|(path, text)| (path.to_string(), text.to_string()))
            .collect(),
    )
    .expect("a change")
}

async fn read(change: &DefinitionChange) -> Result<FactoryDefinition, DefinitionChangeError> {
    match super::read_changed_definition(&base(), change, built_in_step_use).await {
        Ok(definition) => Ok(definition),
        Err(DefinitionChangeReadError::Refused(error)) => Err(error),
        Err(DefinitionChangeReadError::Files(never)) => match never {},
    }
}

/// A change that adds a Flow, and names it in the manifest, reads with the
/// files it leaves as they are.
#[tokio::test]
async fn a_change_reads_over_the_files_it_leaves() {
    let added = change(&[
        (
            "factory.json",
            r#"{ "version": 1, "description": "Ours.", "flows": ["reply", "plan"] }"#,
        ),
        ("flows/plan.json", PLAN),
    ]);

    let definition = read(&added).await.expect("the changed definition reads");

    let flows: Vec<&str> = definition.flows.keys().map(String::as_str).collect();
    assert_eq!(flows, ["plan", "reply"]);
}

/// A change after which the Definition does not read is refused with the
/// validator's own reason.
#[tokio::test]
async fn a_change_the_definition_does_not_read_with_is_refused() {
    let broken = change(&[(
        "agents.json",
        r#"{
            "agents": { "primary": { "bot": "00000000-0000-0000-0000-00000000c1a0" } },
            "roles": { "planner": ["primary"] }
        }"#,
    )]);
    let added = change(&[
        (
            "factory.json",
            r#"{ "version": 1, "flows": ["reply", "plan"] }"#,
        ),
        ("flows/plan.json", PLAN),
        (
            "agents.json",
            r#"{
            "agents": { "primary": { "bot": "00000000-0000-0000-0000-00000000c1a0" } },
            "roles": { "planner": ["primary"] }
        }"#,
        ),
    ]);

    // The `reply` Flow names no Role: only adding `plan`, whose writer no
    // Agent fills, breaks it.
    read(&broken).await.expect("reply names no role");
    let refused = read(&added).await;

    assert!(
        matches!(
            refused,
            Err(DefinitionChangeError::Invalid(DefinitionError::Agents(_)))
        ),
        "{refused:?}"
    );
}

/// A Flow's file the manifest does not name is never read, so nothing
/// would check it: a secret could hide there.
#[tokio::test]
async fn a_file_the_definition_does_not_read_is_refused() {
    let unread = change(&[("flows/plan.json", PLAN)]);
    let elsewhere = change(&[
        ("factory.json", r#"{ "version": 1, "flows": ["reply"] }"#),
        ("README.md", "Our factory."),
    ]);

    let refused = read(&unread).await;
    assert!(
        matches!(&refused, Err(DefinitionChangeError::NotRead(path)) if path == "flows/plan.json"),
        "{refused:?}"
    );
    let refused = read(&elsewhere).await;
    assert!(
        matches!(&refused, Err(DefinitionChangeError::NotRead(path)) if path == "README.md"),
        "{refused:?}"
    );
}

/// A change that writes every file as it is changes nothing.
#[tokio::test]
async fn a_change_that_leaves_every_file_as_it_is_is_refused() {
    let same = change(&[("agents.json", AGENTS), ("factory.json", MANIFEST)]);

    let refused = read(&same).await;

    assert!(
        matches!(refused, Err(DefinitionChangeError::Unchanged)),
        "{refused:?}"
    );
}

/// A change names what it does and writes at least one file, and no more
/// than a Definition has.
#[test]
fn a_change_has_a_title_and_some_files() {
    let files = BTreeMap::from([("agents.json".to_string(), AGENTS.to_string())]);
    let too_many: BTreeMap<String, String> = (0..=MAX_CHANGED_FILES)
        .map(|flow| (format!("flows/f{flow}.json"), REPLY.to_string()))
        .collect();

    let blank = DefinitionChange::new("  ".to_string(), None, files.clone());
    let none = DefinitionChange::new("Change".to_string(), None, BTreeMap::new());
    let many = DefinitionChange::new("Change".to_string(), None, too_many);
    let long = DefinitionChange::new("Change".to_string(), Some("x".repeat(16 * 1024 + 1)), files);

    assert!(matches!(blank, Err(DefinitionChangeError::Title)));
    assert!(matches!(none, Err(DefinitionChangeError::FileCount)));
    assert!(matches!(many, Err(DefinitionChangeError::FileCount)));
    assert!(matches!(long, Err(DefinitionChangeError::Description)));
}
