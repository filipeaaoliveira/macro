use factory_records::domain::models::Outcome;

use super::BuiltInSteps;
use crate::domain::flow::StepName;
use crate::domain::ports::StepExecutor;

#[test]
fn only_reply_is_served() {
    let reply = StepName::try_from("reply".to_string()).expect("valid");
    let plan = StepName::try_from("plan".to_string()).expect("valid");

    assert!(BuiltInSteps.serves(&reply));
    assert!(!BuiltInSteps.serves(&plan));
}

#[tokio::test]
async fn a_reply_is_done_and_answers_the_request() {
    let reply = StepName::try_from("reply".to_string()).expect("valid");

    let report = BuiltInSteps
        .run(&reply, "Summarise the release notes")
        .await
        .expect("reply");

    assert_eq!(report.outcome, Outcome::done());
    assert!(report.output.contains("Summarise the release notes"));
}
