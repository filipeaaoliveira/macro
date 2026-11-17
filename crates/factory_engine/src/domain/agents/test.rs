use factory_records::domain::models::{AgentName, Role};
use serde_json::json;

use super::{AgentDefinition, AgentDefinitionError};

fn role(name: &str) -> Role {
    Role::try_from(name.to_string()).expect("valid role")
}

fn agent(name: &str) -> AgentName {
    AgentName::try_from(name.to_string()).expect("valid agent")
}

fn read(definition: serde_json::Value) -> Result<AgentDefinition, String> {
    serde_json::from_value(definition).map_err(|error| error.to_string())
}

#[test]
fn a_role_takes_its_agents_in_the_order_the_definition_lists_them() {
    let definition = read(json!({
        "agents": {
            "claude": {
                "bot": "00000000-0000-0000-0000-00000000c1a0",
                "model": "claude-opus",
                "instructions": "Work in small commits."
            },
            "codex": { "bot": "00000000-0000-0000-0000-00000000c0de" }
        },
        "roles": {
            "planner": ["codex", "claude"],
            "reviewer": ["claude"]
        }
    }))
    .expect("a definition");

    assert_eq!(
        definition.for_role(&role("planner")),
        [agent("codex"), agent("claude")]
    );
    assert_eq!(definition.for_role(&role("writer")), []);
    let claude = definition.agent(&agent("claude")).expect("claude");
    assert_eq!(claude.model.as_deref(), Some("claude-opus"));
    assert_eq!(
        claude.instructions.as_deref(),
        Some("Work in small commits.")
    );
    assert_eq!(
        definition.agent(&agent("codex")).expect("codex").model,
        None
    );
    assert_eq!(
        definition.fills([&role("planner"), &role("reviewer")]),
        Ok(())
    );
    assert_eq!(
        definition.fills([&role("planner"), &role("writer")]),
        Err(AgentDefinitionError::UnfilledRole(role("writer")))
    );
}

#[test]
fn a_definition_whose_roles_do_not_hold_together_is_refused() {
    let agents = json!({ "claude": { "bot": "00000000-0000-0000-0000-00000000c1a0" } });
    let refused = [
        (json!({ "planner": [] }), "role planner lists no agent"),
        (
            json!({ "planner": ["codex"] }),
            "role planner lists agent codex, which the definition does not have",
        ),
        (
            json!({ "planner": ["claude", "claude"] }),
            "role planner lists agent claude twice",
        ),
    ];

    for (roles, why) in refused {
        let error = read(json!({ "agents": agents, "roles": roles })).expect_err(why);
        assert!(error.contains(why), "{error}");
    }
    assert!(read(json!({ "agents": agents, "roles": {}, "flows": {} })).is_err());
}
