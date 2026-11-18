use super::{BUILT_IN_FLOWS, BuiltInFlow, built_in_roles};
use crate::domain::built_in_steps::{BuiltInSteps, built_in_step_use};
use crate::domain::fakes::{FakeGithub, FakeRepo, FakeWatches};
use crate::domain::ports::StepExecutor;
use crate::domain::pull_request_steps::PullRequestSteps;
use crate::outbound::local_agent::LocalAgent;

/// Each file in `flows/` holds together and says what it does, whichever
/// Steps a worker serves.
#[test]
fn every_built_in_flow_reads_and_describes_itself() {
    for flow in BUILT_IN_FLOWS {
        let read = flow
            .read(|_| Some(crate::domain::flow::StepUse::Work))
            .unwrap_or_else(|error| panic!("flow {} does not read: {error}", flow.name));

        assert!(
            !read.description.trim().is_empty(),
            "flow {} has no description",
            flow.name
        );
        assert_eq!(
            read.check_merges(),
            Ok(()),
            "flow {} merges without a merge gate's approval",
            flow.name
        );
    }
}

/// Each built-in Flow passes the checks a Factory Definition's Flow is
/// held to, as the starter Definition's copies of them must: it names only
/// Steps a worker serves, none past a merge, no Role on a worker's own
/// Step, and merges only through a merge Gate's approval.
#[test]
fn every_built_in_flow_passes_a_definition_s_checks() {
    for flow in BUILT_IN_FLOWS {
        let read = flow
            .read(built_in_step_use)
            .unwrap_or_else(|error| panic!("flow {} does not read: {error}", flow.name));

        assert_eq!(read.check_merges(), Ok(()), "flow {}", flow.name);
        assert_eq!(
            read.check_work_roles(built_in_step_use),
            Ok(()),
            "flow {}",
            flow.name
        );
        assert!(
            read.entries
                .iter()
                .all(|entry| !["deploy", "verify"].contains(&entry.step.as_str())),
            "flow {} runs a Step past its merge",
            flow.name
        );
    }
}

/// The built-in Steps serve every built-in Flow, `code` included.
#[test]
fn the_built_in_steps_run_every_built_in_flow() {
    let steps = BuiltInSteps::new(
        LocalAgent,
        PullRequestSteps::new(
            FakeGithub::default(),
            FakeRepo::default(),
            FakeWatches::default(),
        ),
    );
    let runnable: Vec<&str> = BUILT_IN_FLOWS
        .into_iter()
        .filter(|flow| flow.read(|step| steps.serves(step)).is_ok())
        .map(|flow| flow.name)
        .collect();

    assert_eq!(runnable, ["code", "plan", "reply"]);
}

/// Only `code` works on a repository.
#[test]
fn only_the_code_flow_works_on_a_repository() {
    let on_repository: Vec<&str> = BUILT_IN_FLOWS
        .into_iter()
        .filter(|flow| {
            flow.read(|_| Some(crate::domain::flow::StepUse::Work))
                .is_ok_and(|read| read.repository)
        })
        .map(|flow| flow.name)
        .collect();

    assert_eq!(on_repository, ["code"]);
}

#[test]
fn a_flow_is_found_by_its_name_only() {
    assert_eq!(
        BuiltInFlow::named("plan").map(|flow| flow.name),
        Some("plan")
    );
    assert!(BuiltInFlow::named("plan.json").is_none());
    assert!(BuiltInFlow::named("Plan").is_none());
}

/// Every entry an agent does names the Role it fills, or the Roles of its
/// review panel, and the built-in Flows need six.
#[test]
fn every_agent_entry_names_its_role() {
    for flow in BUILT_IN_FLOWS {
        let read = flow
            .read(|_| Some(crate::domain::flow::StepUse::Work))
            .expect("reads");
        for entry in read.entries {
            let agent_step = ["plan", "write", "review", "fix", "ci_fix", "accept"]
                .contains(&entry.step.as_str());
            assert_eq!(
                entry.role.is_some() || !entry.reviewers.is_empty(),
                agent_step,
                "entry {} of flow {}",
                entry.id,
                flow.name
            );
        }
    }

    let roles: Vec<String> = built_in_roles()
        .expect("roles")
        .into_iter()
        .map(String::from)
        .collect();
    assert_eq!(
        roles,
        [
            "correctness-reviewer",
            "fixer",
            "planner",
            "scope-reviewer",
            "tests-reviewer",
            "verifier",
            "writer"
        ]
    );
}
