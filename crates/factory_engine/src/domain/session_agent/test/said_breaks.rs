use agent_session::domain::routines::RoutineActionStatus;
use factory_records::domain::models::{HandBack, Outcome};

use super::{FakeSessions, agent, plan_task};
use crate::domain::breaks::{Reported, reported};
use crate::domain::fakes::FakeRepo;
use crate::domain::ports::StepAgent;
use crate::domain::session_agent::said_breaks::{FROM_REPLY, missing_breaks};

const SAVES: &str = "BREAK toggle_saves: src/toggle.rs \"saved = true\" -> \"saved = false\" run `cargo test toggle_saves`";
const KEEPS: &str =
    "BREAK toggle_keeps: src/toggle.rs \"keep()\" -> \"drop()\" run `cargo test toggle_keeps`";

/// An agent that wrote its break lines in the reply that ended its turn,
/// not in its hand back, has them added to what the Step hands on, so the
/// checks round reads them; a break the hand back already names, and a
/// line that reads as none, are not added.
#[tokio::test]
async fn break_lines_the_agent_wrote_in_its_reply_reach_the_step_s_answer() {
    let repo = FakeRepo::default();
    let handed = HandBack {
        outcome: Outcome::done(),
        output: format!("Added the toggle and its tests.\n{SAVES}"),
    };
    let mut sessions = FakeSessions::new(&repo, Some(handed), RoutineActionStatus::Succeeded);
    sessions.first_words = Some(format!(
        "I left the BREAK lines out of that hand-back, so they're below.\n\n- {KEEPS}\n{SAVES}"
    ));
    let task = plan_task(&repo).await;

    let answer = agent(&sessions, &repo).answer(task).await.expect("answer");

    assert!(answer.text.starts_with("Added the toggle and its tests."));
    assert!(answer.text.contains(FROM_REPLY), "{}", answer.text);
    let tests: Vec<String> = reported([answer.text.as_str()])
        .iter()
        .map(|kept| {
            assert!(matches!(kept, Reported::Break(_)), "{kept:?}");
            kept.test().to_string()
        })
        .collect();
    assert_eq!(tests, ["toggle_saves", "toggle_keeps"]);
    assert_eq!(answer.text.matches("toggle_saves:").count(), 1);
}

/// Only readable break lines are taken from a reply, the latest for a
/// test, and none whose test the hand back names.
#[test]
fn a_reply_adds_only_readable_breaks_the_hand_back_lacks() {
    let newer = KEEPS.replace("drop()", "skip()");
    let said = format!("BREAK lines follow\n{KEEPS}\n{SAVES}\n{newer}\n");

    assert_eq!(missing_breaks(SAVES, &said), [newer]);
    assert!(missing_breaks(&format!("{SAVES}\n{KEEPS}"), &said).is_empty());
    assert!(missing_breaks("", "Nothing to add.").is_empty());
}
