//! A reviewer's Step with a real [`SessionAgent`] over the fake sessions:
//! the reminder and the re-ask go on in one session, in order.

use agent_session::domain::routines::RoutineActionStatus;
use factory_records::domain::models::{HandBack, Outcome};
use factory_records::domain::review_rounds::ReviewRoundRepository;
use uuid::Uuid;

use super::{PASSES, reviewing, second_round};
use crate::domain::agents::AgentDefinition;
use crate::domain::built_in_steps::BuiltInSteps;
use crate::domain::built_in_steps::review_panel::REASK_PLACE;
use crate::domain::fakes::{FakeGithub, FakeRepo, FakeWatches};
use crate::domain::flow::StepName;
use crate::domain::ports::StepExecutor;
use crate::domain::pull_request_steps::PullRequestSteps;
use crate::domain::session_agent::SessionAgent;
use crate::domain::session_agent::reminder::REMINDER_PLACE;
use crate::domain::session_agent::test::{FakeSessions, LaterTurn};

type Steps = BuiltInSteps<SessionAgent<FakeSessions, FakeRepo>, FakeGithub, FakeRepo, FakeWatches>;

/// The correctness reviewer is `primary`.
fn agents() -> AgentDefinition {
    serde_json::from_value(serde_json::json!({
        "agents": { "primary": { "bot": Uuid::from_u128(0xc1a0) } },
        "roles": { "correctness-reviewer": ["primary"] }
    }))
    .expect("a definition")
}

fn steps(sessions: &FakeSessions, repo: &FakeRepo) -> Steps {
    let github = FakeGithub::default();
    github.state().head = Some("f".repeat(40));
    BuiltInSteps::new(
        SessionAgent::new(sessions.clone(), repo.clone(), agents()),
        PullRequestSteps::new(github, repo.clone(), FakeWatches::default()),
    )
}

fn review() -> StepName {
    StepName::try_from("review".to_string()).expect("valid")
}

/// A later turn in which the reviewer hands back `outcome` with `text`.
fn hands_back(outcome: &str, text: &str) -> LaterTurn {
    LaterTurn {
        handed: HandBack {
            outcome: Outcome::try_from(outcome.to_string()).expect("valid"),
            output: text.to_string(),
        },
        error: None,
    }
}

/// The re-ask comes after the reminder among the Step's later prompts, so
/// a reviewer reminded first can still be asked again.
#[test]
fn the_re_ask_comes_after_the_reminder() {
    assert!(REASK_PLACE > REMINDER_PLACE);
}

/// A reviewer that hands nothing back is reminded; one whose answer to the
/// reminder then does not read is asked again, once, in the same session,
/// and what it hands back then is its answer.
#[tokio::test]
async fn a_reviewer_reminded_first_is_still_asked_again() {
    let repo = FakeRepo::default();
    let (run_id, round) = second_round(&repo, &[], true).await;
    let sessions = FakeSessions::new(&repo, None, RoutineActionStatus::Succeeded).later(&[
        hands_back("pass", "Looks good to me."),
        hands_back("pass", PASSES),
    ]);

    let report = steps(&sessions, &repo)
        .run(&review(), reviewing(run_id, &round))
        .await
        .expect("review");

    assert_eq!(report.outcome.as_str(), "pass");
    let later = sessions.later_prompts();
    assert_eq!(later.len(), 2);
    assert!(
        later[0]
            .prompt
            .starts_with("## Your turn ended without a result"),
        "{}",
        later[0].prompt
    );
    assert!(
        later[1].prompt.contains("did not read"),
        "{}",
        later[1].prompt
    );
    assert_ne!(later[0].action.action_id, later[1].action.action_id);
    let rounds = repo.review_rounds(run_id).await.expect("rounds");
    assert!(rounds[1].findings.is_empty());
    assert_eq!(rounds[1].answered, [5]);
}
