use super::{BUILT_IN_FLOWS, BuiltInFlow};
use crate::domain::built_in_steps::BuiltInSteps;
use crate::domain::ports::StepExecutor;
use crate::outbound::local_agent::LocalAgent;

/// Each file in `flows/` holds together and says what it does, whichever
/// Steps a worker serves.
#[test]
fn every_built_in_flow_reads_and_describes_itself() {
    for flow in BUILT_IN_FLOWS {
        let read = flow
            .read(|_| true)
            .unwrap_or_else(|error| panic!("flow {} does not read: {error}", flow.name));

        assert!(
            !read.description.trim().is_empty(),
            "flow {} has no description",
            flow.name
        );
    }
}

/// The built-in Steps serve `plan` and `reply`; `code` names Steps no
/// worker serves yet.
#[test]
fn the_built_in_steps_run_the_plan_and_reply_flows_but_not_code() {
    let steps = BuiltInSteps::new(LocalAgent);
    let runnable: Vec<&str> = BUILT_IN_FLOWS
        .into_iter()
        .filter(|flow| flow.read(|step| steps.serves(step)).is_ok())
        .map(|flow| flow.name)
        .collect();

    assert_eq!(runnable, ["plan", "reply"]);
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
