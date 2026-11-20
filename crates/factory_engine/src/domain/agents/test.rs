use bot_id::BotId;
use factory_records::domain::models::{AgentName, Role};
use factory_records::domain::standing_acts::{StandingAct, StandingRoleName};
use serde_json::json;
use uuid::Uuid;

use super::{AgentDefinition, AgentDefinitionError, StandingCheck};

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

/// A definition written for the single reviewer keeps working: the review
/// panel's Roles it does not list take `reviewer`'s Agents, one it lists
/// takes its own, and one filled by neither stops the service with a
/// message that names both.
#[test]
fn the_review_panel_roles_default_to_the_reviewer() {
    let definition = |roles: serde_json::Value| {
        read(json!({
            "agents": {
                "primary": { "bot": "00000000-0000-0000-0000-00000000c1a0" },
                "backup": { "bot": "00000000-0000-0000-0000-00000000c0de" }
            },
            "roles": roles
        }))
        .expect("a definition")
    };
    let single = definition(json!({
        "planner": ["primary"],
        "writer": ["primary"],
        "reviewer": ["backup", "primary"],
        "fixer": ["primary"]
    }));
    let own_tests = definition(json!({
        "planner": ["primary"],
        "writer": ["primary"],
        "reviewer": ["backup"],
        "tests-reviewer": ["primary"],
        "verifier": ["primary"],
        "fixer": ["primary"]
    }));
    let no_reviewer = definition(json!({
        "planner": ["primary"],
        "writer": ["primary"],
        "correctness-reviewer": ["primary"],
        "fixer": ["primary"]
    }));

    assert_eq!(single.fills_built_in_flows(), Ok(()));
    assert_eq!(
        single.for_role(&role("scope-reviewer")),
        [agent("backup"), agent("primary")]
    );
    assert_eq!(
        own_tests.for_role(&role("tests-reviewer")),
        [agent("primary")]
    );
    assert_eq!(
        own_tests.for_role(&role("correctness-reviewer")),
        [agent("backup")]
    );
    assert_eq!(own_tests.for_role(&role("other-reviewer")), []);
    // The verifier takes `reviewer`'s Agents unless the definition lists
    // its own, so a definition written before it keeps working.
    assert_eq!(
        single.for_role(&role("verifier")),
        [agent("backup"), agent("primary")]
    );
    assert_eq!(own_tests.for_role(&role("verifier")), [agent("primary")]);

    let refused = no_reviewer.fills_built_in_flows();
    assert_eq!(
        refused,
        Err(AgentDefinitionError::UnfilledReviewer(role(
            "scope-reviewer"
        )))
    );
    let message = refused.expect_err("refused").to_string();
    assert!(message.contains("scope-reviewer") && message.contains("`reviewer`"));
}

/// Runs started on the single-reviewer `code` Flow review with `reviewer`
/// until they end, so a definition that fills only the panel's Roles is
/// refused.
#[test]
fn the_reviewer_stays_required_for_runs_in_flight() {
    let definition = read(json!({
        "agents": { "primary": { "bot": "00000000-0000-0000-0000-00000000c1a0" } },
        "roles": {
            "planner": ["primary"],
            "writer": ["primary"],
            "correctness-reviewer": ["primary"],
            "scope-reviewer": ["primary"],
            "tests-reviewer": ["primary"],
            "verifier": ["primary"],
            "fixer": ["primary"]
        }
    }))
    .expect("a definition");

    assert_eq!(
        definition.fills_built_in_flows(),
        Err(AgentDefinitionError::UnfilledRole(role("reviewer")))
    );
}

fn standing_role(name: &str) -> StandingRoleName {
    StandingRoleName::try_from(name.to_string()).expect("valid standing role")
}

fn bot(id: &str) -> BotId {
    BotId::new_from_uuid(Uuid::parse_str(id).expect("bot id"))
}

const PRIMARY_BOT: &str = "00000000-0000-0000-0000-00000000c1a0";
const BACKUP_BOT: &str = "00000000-0000-0000-0000-00000000c0de";
const OTHER_BOT: &str = "00000000-0000-0000-0000-0000000000ff";

/// A definition with `standing` as its Standing Roles, and the Agents
/// `primary` and `backup`.
fn with_standing(standing: serde_json::Value) -> Result<AgentDefinition, String> {
    read(json!({
        "agents": {
            "primary": { "bot": PRIMARY_BOT },
            "backup": { "bot": BACKUP_BOT }
        },
        "roles": { "planner": ["primary"] },
        "standing": standing
    }))
}

/// One Agent may fill every Standing Role, or each Standing Role its own;
/// a bot does an act only through a Standing Role that allows it, and a
/// definition with no `standing` section leaves every act open.
#[test]
fn a_bot_does_an_act_only_through_a_standing_role_that_allows_it() {
    let one_agent = with_standing(json!({
        "intake": { "acts": ["start", "link"], "agents": ["primary"] },
        "gatekeeper": { "acts": ["answer_plan", "answer_blockers"], "agents": ["primary"] },
        "operator": { "acts": ["cancel"], "agents": ["primary"] }
    }))
    .expect("one agent in every standing role");
    let own_agents = with_standing(json!({
        "intake": {
            "description": "Starts runs from requests",
            "acts": ["start", "link"],
            "agents": ["backup"]
        },
        "gatekeeper": { "acts": ["answer_plan"], "agents": ["primary"] }
    }))
    .expect("an agent per standing role");
    let open = read(json!({
        "agents": { "primary": { "bot": PRIMARY_BOT } },
        "roles": { "planner": ["primary"] }
    }))
    .expect("no standing section");

    assert_eq!(
        one_agent.standing_check(StandingAct::Cancel, Some(bot(PRIMARY_BOT))),
        StandingCheck::Filled {
            role: standing_role("operator"),
            agent: agent("primary"),
        }
    );
    assert_eq!(
        own_agents.standing_check(StandingAct::AnswerPlan, Some(bot(PRIMARY_BOT))),
        StandingCheck::Filled {
            role: standing_role("gatekeeper"),
            agent: agent("primary"),
        }
    );
    assert_eq!(
        own_agents.standing_check(StandingAct::AnswerPlan, Some(bot(BACKUP_BOT))),
        StandingCheck::Refused {
            allowing: vec![standing_role("gatekeeper")],
        }
    );
    assert_eq!(
        own_agents.standing_check(StandingAct::Start, Some(bot(OTHER_BOT))),
        StandingCheck::Refused {
            allowing: vec![standing_role("intake")],
        }
    );
    assert_eq!(
        own_agents.standing_check(StandingAct::Cancel, Some(bot(PRIMARY_BOT))),
        StandingCheck::Refused { allowing: vec![] }
    );
    assert_eq!(
        own_agents.standing_check(StandingAct::AnswerPlan, None),
        StandingCheck::Refused {
            allowing: vec![standing_role("gatekeeper")],
        }
    );
    assert_eq!(
        open.standing_check(StandingAct::AnswerPlan, Some(bot(OTHER_BOT))),
        StandingCheck::Open
    );
}

/// The check refuses an unknown act, any merge, a Standing Role no Agent
/// fills, an Agent the definition does not have, and an act or an Agent
/// listed twice, naming the Standing Role and the problem.
#[test]
fn standing_roles_that_do_not_hold_together_are_refused() {
    let refused = [
        (
            json!({ "intake": { "acts": ["start", "deploy"], "agents": ["primary"] } }),
            "standing role intake names act \"deploy\", which is not one of",
        ),
        (
            json!({ "release": { "acts": ["merge"], "agents": ["primary"] } }),
            "standing role release names act \"merge\": no standing role merges",
        ),
        (
            json!({ "gatekeeper": { "acts": ["answer_merge"], "agents": ["primary"] } }),
            "standing role gatekeeper names act \"answer_merge\": no standing role merges",
        ),
        (
            json!({ "intake": { "acts": [], "agents": ["primary"] } }),
            "standing role intake allows no act",
        ),
        (
            json!({ "intake": { "acts": ["start"], "agents": [] } }),
            "standing role intake lists no agent",
        ),
        (
            json!({ "intake": { "acts": ["start"], "agents": ["stranger"] } }),
            "standing role intake lists agent stranger, which the definition does not have",
        ),
        (
            json!({ "intake": { "acts": ["start", "start"], "agents": ["primary"] } }),
            "standing role intake names act start twice",
        ),
        (
            json!({ "intake": { "acts": ["start"], "agents": ["primary", "primary"] } }),
            "standing role intake lists agent primary twice",
        ),
        (
            json!({ "intake": { "acts": ["start"], "agents": ["primary"], "bots": [] } }),
            "unknown field `bots`",
        ),
    ];

    let null = with_standing(serde_json::Value::Null).expect_err("a null standing section");
    assert!(null.contains("invalid type: null"), "{null}");

    for (standing, why) in refused {
        let error = with_standing(standing).expect_err(why);
        assert!(error.contains(why), "{error}");
    }
}

/// An empty `standing` section is Standing Roles that allow nothing: every
/// agent act is refused, naming no Standing Role.
#[test]
fn an_empty_standing_section_refuses_every_agent_act() {
    let definition = with_standing(json!({})).expect("an empty standing section");

    assert!(definition.has_standing_roles());
    for act in [
        StandingAct::Start,
        StandingAct::Cancel,
        StandingAct::Link,
        StandingAct::AnswerPlan,
        StandingAct::AnswerBlockers,
    ] {
        assert_eq!(
            definition.standing_check(act, Some(bot(PRIMARY_BOT))),
            StandingCheck::Refused { allowing: vec![] }
        );
    }
}

/// A Run keeps its Definition's Standing Roles as written, and reads them
/// back the same.
#[test]
fn standing_roles_read_back_as_kept() {
    let definition = with_standing(json!({
        "gatekeeper": {
            "description": "Approves plans",
            "acts": ["answer_plan", "answer_blockers"],
            "agents": ["backup", "primary"]
        }
    }))
    .expect("a definition");

    let kept = serde_json::to_value(&definition).expect("kept");
    let read_back: AgentDefinition = serde_json::from_value(kept).expect("read back");

    assert_eq!(read_back, definition);
}

/// Standing Roles are read only from a Factory Definition: the
/// deployment's `FACTORY_AGENTS` with a `standing` section is refused, as
/// no Run without a Definition would check it.
#[test]
fn the_deployments_agents_take_no_standing_roles() {
    let definition = read(json!({
        "agents": { "primary": { "bot": PRIMARY_BOT } },
        "roles": {
            "planner": ["primary"],
            "writer": ["primary"],
            "reviewer": ["primary"],
            "fixer": ["primary"]
        },
        "standing": { "operator": { "acts": ["cancel"], "agents": ["primary"] } }
    }))
    .expect("a definition");

    assert_eq!(
        AgentDefinition::configured(Some(definition), None),
        Err(AgentDefinitionError::StandingOutsideDefinition)
    );
}

/// A definition with `rules` as its Agent Rules, over `primary` and
/// `backup`.
fn ruled(rules: serde_json::Value) -> AgentDefinition {
    read(json!({
        "agents": {
            "primary": { "bot": "00000000-0000-0000-0000-00000000c1a0" },
            "backup": { "bot": "00000000-0000-0000-0000-00000000c0de" }
        },
        "roles": { "planner": ["primary", "backup"], "writer": ["primary", "backup"] },
        "rules": rules
    }))
    .expect("a definition with agent rules")
}

fn tags(tags: &[&str]) -> Vec<String> {
    tags.iter().map(ToString::to_string).collect()
}

/// The (Role, rule, Agent) of each rule chosen.
fn picked(
    chosen: &[factory_records::domain::evaluation::RunAgentRule],
) -> Vec<(String, String, String)> {
    chosen
        .iter()
        .map(|rule| {
            (
                rule.role.to_string(),
                rule.rule.clone(),
                rule.agent.to_string(),
            )
        })
        .collect()
}

/// A rule on a tag chooses its Agent for a Run whose task carries the tag,
/// whatever its case; a task without it keeps the Role's own order.
#[test]
fn an_agent_rule_matches_a_task_tag() {
    use super::RuleSubject;

    let definition = ruled(json!([
        { "name": "docs", "role": "writer", "tag": "docs", "agent": "backup" }
    ]));
    let docs = tags(&["urgent", " Docs "]);
    let other = tags(&["urgent"]);

    assert!(definition.has_tag_rules());
    assert_eq!(
        picked(&definition.chosen_by_rules(
            RuleSubject {
                tags: &docs,
                repository: None
            },
            |_| false
        )),
        [("writer".into(), "docs".into(), "backup".into())]
    );
    assert_eq!(
        definition.chosen_by_rules(
            RuleSubject {
                tags: &other,
                repository: Some("https://github.com/acme/site")
            },
            |_| false
        ),
        []
    );
    assert_eq!(
        definition.chosen_by_rules(RuleSubject::default(), |_| false),
        []
    );
}

/// A rule on a repository chooses its Agent for a Run on it, whatever the
/// case of its owner and name; a rule naming a tag and a repository needs
/// both.
#[test]
fn an_agent_rule_matches_the_run_s_repository() {
    use super::RuleSubject;

    let definition = ruled(json!([
        { "name": "site-docs", "role": "writer", "tag": "docs", "repository": "acme/site", "agent": "primary" },
        { "name": "site", "role": "writer", "repository": "acme/site", "agent": "backup" },
        { "name": "plans", "role": "planner", "repository": "Acme/Site", "agent": "backup" }
    ]));
    let none = tags(&[]);
    let docs = tags(&["docs"]);
    let on_site = |tags| RuleSubject {
        tags,
        repository: Some("https://github.com/ACME/site"),
    };

    assert!(
        !ruled(json!([
            { "name": "site", "role": "writer", "repository": "acme/site", "agent": "backup" }
        ]))
        .has_tag_rules()
    );
    // The first rule that matches a Role chooses its Agent.
    assert_eq!(
        picked(&definition.chosen_by_rules(on_site(&none), |_| false)),
        [
            ("writer".into(), "site".into(), "backup".into()),
            ("planner".into(), "plans".into(), "backup".into()),
        ]
    );
    assert_eq!(
        picked(&definition.chosen_by_rules(on_site(&docs), |_| false)),
        [
            ("writer".into(), "site-docs".into(), "primary".into()),
            ("planner".into(), "plans".into(), "backup".into()),
        ]
    );
    assert_eq!(
        definition.chosen_by_rules(
            RuleSubject {
                tags: &docs,
                repository: Some("https://github.com/acme/app")
            },
            |_| false
        ),
        []
    );
    // A Role an Experiment arm chose for takes no rule.
    assert_eq!(
        picked(&definition.chosen_by_rules(on_site(&docs), |role| role.as_str() == "writer")),
        [("planner".into(), "plans".into(), "backup".into())]
    );
}

/// `FACTORY_AGENTS` takes no Agent Rules: only a Factory Definition's Runs
/// match them.
#[test]
fn the_deployment_s_agents_take_no_agent_rules() {
    let definition = ruled(json!([
        { "name": "docs", "role": "writer", "tag": "docs", "agent": "backup" }
    ]));

    assert_eq!(
        AgentDefinition::configured(Some(definition), None),
        Err(AgentDefinitionError::RulesOutsideDefinition)
    );
}

/// A definition's Agent Rules are kept with a Run as it reads them, and a
/// definition with none writes no `rules`, so Runs kept before them read
/// as before.
#[test]
fn agent_rules_are_written_as_they_are_read() {
    let definition = ruled(json!([
        { "name": "docs", "role": "writer", "tag": "docs", "agent": "backup" }
    ]));
    let written = serde_json::to_value(&definition).expect("written");
    assert_eq!(read(written.clone()).expect("read back"), definition);
    assert_eq!(
        written["rules"],
        json!([{ "name": "docs", "role": "writer", "tag": "docs", "agent": "backup" }])
    );

    let without = read(json!({
        "agents": { "primary": { "bot": "00000000-0000-0000-0000-00000000c1a0" } },
        "roles": { "writer": ["primary"] }
    }))
    .expect("a definition");
    assert!(
        serde_json::to_value(&without)
            .expect("written")
            .get("rules")
            .is_none()
    );
}
