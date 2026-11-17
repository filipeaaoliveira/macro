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
            "primary": {
                "bot": "00000000-0000-0000-0000-00000000c1a0",
                "model": "large-model",
                "instructions": "Work in small commits."
            },
            "backup": { "bot": "00000000-0000-0000-0000-00000000c0de" }
        },
        "roles": {
            "planner": ["backup", "primary"],
            "reviewer": ["primary"]
        }
    }))
    .expect("a definition");

    assert_eq!(
        definition.for_role(&role("planner")),
        [agent("backup"), agent("primary")]
    );
    assert_eq!(definition.for_role(&role("writer")), []);
    let primary = definition.agent(&agent("primary")).expect("primary");
    assert_eq!(primary.model.as_deref(), Some("large-model"));
    assert_eq!(
        primary.instructions.as_deref(),
        Some("Work in small commits.")
    );
    assert_eq!(
        definition.agent(&agent("backup")).expect("backup").model,
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
    let agents = json!({ "primary": { "bot": "00000000-0000-0000-0000-00000000c1a0" } });
    let refused = [
        (json!({ "planner": [] }), "role planner lists no agent"),
        (
            json!({ "planner": ["backup"] }),
            "role planner lists agent backup, which the definition does not have",
        ),
        (
            json!({ "planner": ["primary", "primary"] }),
            "role planner lists agent primary twice",
        ),
    ];

    for (roles, why) in refused {
        let error = read(json!({ "agents": agents, "roles": roles })).expect_err(why);
        assert!(error.contains(why), "{error}");
    }
    assert!(read(json!({ "agents": agents, "roles": {}, "flows": {} })).is_err());
}

/// The service starts only with a definition that fills every Role the
/// built-in Flows name.
#[test]
fn a_definition_must_fill_every_role_of_the_built_in_flows() {
    let definition = |roles: serde_json::Value| {
        read(json!({
            "agents": { "primary": { "bot": "00000000-0000-0000-0000-00000000c1a0" } },
            "roles": roles
        }))
        .expect("a definition")
    };
    let full = definition(json!({
        "planner": ["primary"],
        "writer": ["primary"],
        "reviewer": ["primary"],
        "fixer": ["primary"]
    }));
    let no_fixer = definition(json!({
        "planner": ["primary"],
        "writer": ["primary"],
        "reviewer": ["primary"]
    }));

    assert_eq!(full.fills_built_in_flows(), Ok(()));
    assert_eq!(
        no_fixer.fills_built_in_flows(),
        Err(AgentDefinitionError::UnfilledRole(role("fixer")))
    );
}

/// The service runs with the definition it is given, on the stand-in with
/// none, and refuses the retired single agent id rather than silently doing
/// no work.
#[test]
fn the_retired_agent_id_alone_is_refused() {
    let definition = read(json!({
        "agents": { "primary": { "bot": "00000000-0000-0000-0000-00000000c1a0" } },
        "roles": {
            "planner": ["primary"],
            "writer": ["primary"],
            "reviewer": ["primary"],
            "fixer": ["primary"]
        }
    }))
    .expect("a definition");
    let retired = Some("00000000-0000-0000-0000-00000000c1a0");

    assert_eq!(
        AgentDefinition::configured(None, retired),
        Err(AgentDefinitionError::RetiredAgentId)
    );
    assert_eq!(AgentDefinition::configured(None, None), Ok(None));
    assert_eq!(
        AgentDefinition::configured(Some(definition.clone()), retired),
        Ok(Some(definition))
    );
}
